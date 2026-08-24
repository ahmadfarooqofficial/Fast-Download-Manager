<#
.SYNOPSIS
    Authenticode-signs FDM's binaries so Windows shows "Ahmad Farooq" instead of
    "Publisher: Unknown".

.DESCRIPTION
    Read this part before anything else, because it is the thing people get wrong:

    Windows does NOT read the publisher name from the file's version resource.
    FDM already sets VersionInfoCompany to "Ahmad Farooq" -- that is what shows in
    right-click > Properties > Details. The UAC elevation dialog and SmartScreen
    read something completely different: the subject name of the Authenticode
    certificate the file is signed with. An unsigned file is "Publisher: Unknown"
    no matter what its version resource says, and no build flag changes that.

    So there are exactly two ways forward.

    (1) A real code-signing certificate. This is the only option that works on
        other people's computers.

        Since June 2023 the CA/Browser Forum requires the private key to live on
        hardware (a USB token) or in a cloud HSM, so you cannot just download a
        .pfx any more. Relevant options for an open-source project:

          - Certum Open Source Code Signing   ~EUR 30-90/yr, cheapest by far,
                                              requires proof the project is open
                                              source (FDM is MIT, so it qualifies)
          - Sectigo / SSL.com OV              ~USD 200-400/yr
          - Any EV certificate                ~USD 300-600/yr, and it is the only
                                              thing that skips the SmartScreen
                                              "not commonly downloaded" warning
                                              from day one

        A non-EV certificate still shows a SmartScreen warning until the
        certificate accumulates download reputation. That is normal and it fades.

    (2) A self-signed certificate (-SelfSigned). Signs correctly and will show
        "Ahmad Farooq" on THIS machine once the certificate is trusted locally.
        On any other machine it is worse than unsigned, because it looks like a
        forged signature. Use it to verify the pipeline, never to ship.

.PARAMETER Pfx
    Path to a .pfx/.p12 containing the code-signing key.

.PARAMETER Password
    Password for -Pfx. Prompted for securely if omitted.

.PARAMETER Thumbprint
    SHA1 thumbprint of a certificate already in the Windows certificate store.
    This is what you use with a hardware token or cloud HSM, where the key can
    never be exported to a file.

.PARAMETER SelfSigned
    Create (or reuse) a local development certificate for "Ahmad Farooq".
    Development only. See above.

.PARAMETER TrustSelfSigned
    Also install the self-signed certificate into the machine's Trusted Root
    store, which is what makes UAC display the name instead of a red warning.
    Requires administrator. Only ever do this on your own dev machine.

.PARAMETER Files
    Files to sign. Defaults to every FDM binary and the built installer.

.EXAMPLE
    # See it work today, on this machine only
    powershell -ExecutionPolicy Bypass -File scripts\sign.ps1 -SelfSigned -TrustSelfSigned

.EXAMPLE
    # Real release, key on a hardware token
    powershell -ExecutionPolicy Bypass -File scripts\sign.ps1 -Thumbprint A1B2C3...
#>

[CmdletBinding(DefaultParameterSetName = 'SelfSigned')]
param(
    [Parameter(ParameterSetName = 'Pfx', Mandatory)][string]$Pfx,
    [Parameter(ParameterSetName = 'Pfx')][string]$Password,

    [Parameter(ParameterSetName = 'Store', Mandatory)][string]$Thumbprint,

    [Parameter(ParameterSetName = 'SelfSigned')][switch]$SelfSigned,
    [Parameter(ParameterSetName = 'SelfSigned')][switch]$TrustSelfSigned,

    [string[]]$Files,
    [string]$Subject = 'Ahmad Farooq',
    # DigiCert's RFC3161 endpoint. A timestamp is what keeps the signature valid
    # after the certificate expires -- without one, every copy of the installer
    # ever downloaded becomes untrusted on the certificate's expiry date.
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Step { param([string]$m) Write-Host "`n==> $m" -ForegroundColor Red }
function Write-Info { param([string]$m) Write-Host "    $m" -ForegroundColor Gray }
function Write-Ok   { param([string]$m) Write-Host "    OK  $m" -ForegroundColor Green }
function Write-Warn { param([string]$m) Write-Host "    !   $m" -ForegroundColor Yellow }
function Fail { param([string]$m) Write-Host "`nSIGNING FAILED: $m" -ForegroundColor Red; exit 1 }

$RepoRoot = Split-Path -Parent $PSScriptRoot

# Run a native command, capture stdout+stderr, return the exit code by reference.
#
# Load-bearing: with $ErrorActionPreference = 'Stop', ANY line a native .exe
# writes to stderr becomes a terminating PowerShell error. signtool writes to
# stderr routinely -- notably "certificate chain ... terminated in a root
# certificate which is not trusted", which is the expected result for a
# self-signed dev certificate. Without this, that expected warning aborts the
# script before the code below can decide whether it mattered.
function Invoke-Native {
    param([Parameter(Mandatory)][string]$Exe, [Parameter(Mandatory)][string[]]$Arguments)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Exe @Arguments 2>&1
        return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = @($out | ForEach-Object { "$_" }) }
    } finally {
        $ErrorActionPreference = $prev
    }
}

# ------------------------------------------------------------------- signtool

function Find-SignTool {
    # Newest SDK first. Version-sort the directory names properly rather than
    # lexically, or 10.0.9 would beat 10.0.26100.
    $roots = @(
        "${env:ProgramFiles(x86)}\Windows Kits\10\bin",
        "$env:ProgramFiles\Windows Kits\10\bin"
    )
    $found = foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^10\.\d+\.\d+\.\d+$' } |
            Sort-Object { [version]$_.Name } -Descending |
            ForEach-Object {
                $p = Join-Path $_.FullName 'x64\signtool.exe'
                if (Test-Path -LiteralPath $p) { $p }
            }
    }
    if ($found) { return @($found)[0] }

    $onPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

Write-Host ''
Write-Host '  FDM - Fast Download Manager :: code signing' -ForegroundColor White
Write-Host '  ------------------------------------------' -ForegroundColor DarkGray

Write-Step 'Locating signtool'
$SignTool = Find-SignTool
if (-not $SignTool) {
    Fail @'
signtool.exe not found. It ships with the Windows SDK:
    winget install Microsoft.WindowsSDK.10.0.26100
'@
}
Write-Ok $SignTool

# ---------------------------------------------------------------- what to sign
# Order matters: the inner binaries must be signed BEFORE the installer is
# compiled, because the installer embeds them. Signing the installer afterwards
# does not retroactively sign its payload.

Write-Step 'Collecting files'

if (-not $Files) {
    $candidates = @(
        "$RepoRoot\target\release\fdm.exe",
        "$RepoRoot\target\release\fdm-desktop.exe",
        "$RepoRoot\target\release\fdm-host.exe",
        "$RepoRoot\target\installer-staging\fdm.exe",
        "$RepoRoot\target\installer-staging\fdm-desktop.exe",
        "$RepoRoot\target\installer-staging\fdm-host.exe"
    )
    $candidates += (Get-ChildItem "$RepoRoot\installer\output\FDM-Setup-*.exe" -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName)
    $Files = @($candidates | Where-Object { $_ -and (Test-Path -LiteralPath $_) })
}

if (-not $Files) { Fail 'Nothing to sign. Build first: scripts\build-installer.ps1' }
foreach ($f in $Files) { Write-Info ($f -replace [regex]::Escape($RepoRoot + '\'), '') }

# ------------------------------------------------------------------ credential

Write-Step 'Preparing certificate'

$signArgs = @('sign', '/fd', 'SHA256', '/tr', $TimestampUrl, '/td', 'SHA256', '/v')
$isSelfSigned = $false

switch ($PSCmdlet.ParameterSetName) {

    'Pfx' {
        if (-not (Test-Path -LiteralPath $Pfx)) { Fail "PFX not found: $Pfx" }
        if (-not $Password) {
            $secure = Read-Host -AsSecureString "Password for $(Split-Path -Leaf $Pfx)"
            $Password = [Runtime.InteropServices.Marshal]::PtrToStringAuto(
                [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure))
        }
        $signArgs += @('/f', $Pfx, '/p', $Password)
        Write-Ok "Using PFX $(Split-Path -Leaf $Pfx)"
    }

    'Store' {
        $cert = Get-ChildItem Cert:\CurrentUser\My, Cert:\LocalMachine\My -ErrorAction SilentlyContinue |
            Where-Object { $_.Thumbprint -eq ($Thumbprint -replace '\s', '') }
        if (-not $cert) { Fail "No certificate with thumbprint $Thumbprint in CurrentUser\My or LocalMachine\My." }
        $signArgs += @('/sha1', ($Thumbprint -replace '\s', ''))
        Write-Ok "Using store certificate: $($cert[0].Subject)"
    }

    'SelfSigned' {
        $isSelfSigned = $true
        Write-Warn 'DEVELOPMENT CERTIFICATE. Valid on this machine only.'
        Write-Warn 'Never publish a self-signed build -- to other users it looks forged.'

        $existing = Get-ChildItem Cert:\CurrentUser\My |
            Where-Object {
                $_.Subject -eq "CN=$Subject" -and
                $_.NotAfter -gt (Get-Date) -and
                $_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.3'   # id-kp-codeSigning
            } | Sort-Object NotAfter -Descending | Select-Object -First 1

        if ($existing) {
            $cert = $existing
            Write-Info "Reusing existing dev certificate (expires $($cert.NotAfter.ToString('yyyy-MM-dd')))"
        } else {
            Write-Info "Creating dev certificate CN=$Subject ..."
            $cert = New-SelfSignedCertificate `
                -Subject "CN=$Subject" `
                -Type CodeSigningCert `
                -KeyAlgorithm RSA -KeyLength 3072 `
                -HashAlgorithm SHA256 `
                -CertStoreLocation Cert:\CurrentUser\My `
                -NotAfter (Get-Date).AddYears(3)
            Write-Ok "Created, thumbprint $($cert.Thumbprint)"
        }

        if ($TrustSelfSigned) {
            $admin = ([Security.Principal.WindowsPrincipal] `
                [Security.Principal.WindowsIdentity]::GetCurrent()
            ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

            if (-not $admin) {
                Write-Warn 'Not elevated -- cannot install into Trusted Root.'
                Write-Warn 'UAC will still say "Unknown" until the certificate is trusted.'
                Write-Warn 'Re-run this from an elevated PowerShell to finish that step.'
            } else {
                $root = New-Object Security.Cryptography.X509Certificates.X509Store 'Root', 'LocalMachine'
                $root.Open('ReadWrite')
                $root.Add($cert)
                $root.Close()
                Write-Ok 'Installed into LocalMachine\Root (this machine now trusts it)'
                Write-Warn "Undo later with: Get-ChildItem Cert:\LocalMachine\Root | ? Thumbprint -eq '$($cert.Thumbprint)' | Remove-Item"
            }
        } else {
            Write-Warn 'Pass -TrustSelfSigned to make Windows actually display the name.'
        }

        $signArgs += @('/sha1', $cert.Thumbprint)
    }
}

# ----------------------------------------------------------------------- sign

Write-Step 'Signing'

$failed = @()
foreach ($f in $Files) {
    $short = $f -replace [regex]::Escape($RepoRoot + '\'), ''
    $r = Invoke-Native $SignTool ($signArgs + @($f))
    if ($r.ExitCode -ne 0) {
        $failed += $short
        Write-Host "    FAIL  $short" -ForegroundColor Red
        $r.Output | ForEach-Object { Write-Host "          $_" -ForegroundColor DarkRed }
    } else {
        Write-Ok $short
    }
}
if ($failed) { Fail "Could not sign: $($failed -join ', ')" }

# ---------------------------------------------------------------------- verify

Write-Step 'Verifying'

foreach ($f in $Files) {
    $short = $f -replace [regex]::Escape($RepoRoot + '\'), ''
    # /pa = use the Authenticode policy, i.e. check it the way Windows will.
    $r = Invoke-Native $SignTool @('verify', '/pa', '/v', $f)
    if ($r.ExitCode -eq 0) {
        Write-Ok "$short  (Windows will accept this signature)"
    } elseif ($isSelfSigned) {
        # Expected: the dev certificate is not in a trusted root store, so
        # Authenticode policy rejects the chain. The signature itself is fine.
        Write-Warn "$short  signature present but chain untrusted -- expected for self-signed"
    } else {
        Write-Host "    FAIL  $short" -ForegroundColor Red
        $r.Output | Select-Object -Last 6 | ForEach-Object { Write-Host "          $_" -ForegroundColor DarkRed }
        $failed += $short
    }
}
if ($failed) { Fail "Signature verification failed: $($failed -join ', ')" }

Write-Host ''
if ($isSelfSigned) {
    Write-Host '  Done -- development signature applied.' -ForegroundColor Yellow
    Write-Host '  Right-click the setup file > Properties > Digital Signatures to confirm' -ForegroundColor Gray
    Write-Host "  it reads $Subject." -ForegroundColor Gray
    Write-Host ''
    Write-Host '  For real distribution you need a purchased certificate. For an MIT' -ForegroundColor Gray
    Write-Host '  open-source project the cheapest route is Certum Open Source Code' -ForegroundColor Gray
    Write-Host '  Signing; then re-run this script with -Thumbprint.' -ForegroundColor Gray
} else {
    Write-Host "  Done -- signed as $Subject and timestamped." -ForegroundColor Green
    Write-Host '  Note: unless the certificate is EV, SmartScreen will still warn until' -ForegroundColor Gray
    Write-Host '  the certificate builds download reputation. That is expected and fades.' -ForegroundColor Gray
}
Write-Host ''
