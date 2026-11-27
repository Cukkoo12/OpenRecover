<#
.SYNOPSIS
Builds an x64 MSIX package for OpenRecover (unsigned for Microsoft Store submission by default,
or optionally self-signed for local sideload verification).

.EXAMPLE
# Unsigned package ready for Microsoft Partner Center upload:
.\packaging\msix\build_msix.ps1 `
    -IdentityName "<YourPartnerCenterPackageIdentityName>" `
    -Publisher "CN=<YourPartnerCenterPublisherId>" `
    -PublisherDisplayName "<YourPublisherDisplayName>" `
    -Version "0.1.0.0"

.EXAMPLE
# Self-signed local test build:
.\packaging\msix\build_msix.ps1 -SignSelfSigned
#>
[CmdletBinding()]
param(
    [string]$IdentityName = "OpenRecover.OpenRecover.Dev",
    [string]$Publisher = "CN=OpenRecoverDev",
    [string]$PublisherDisplayName = "OpenRecover Contributors",
    [string]$Version = "0.1.0.0",
    [switch]$IncludeDonateButton,
    [switch]$DynamicCrt,
    [switch]$SkipBuild,
    [switch]$SignSelfSigned
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($Version -notmatch '^\d+\.\d+\.\d+\.0$') {
    throw "Microsoft Store MSIX Version must be formatted as Major.Minor.Build.0 (received '$Version')."
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Resolve-Path (Join-Path $ScriptDir "..\..")

# Locate newest installed Windows 10/11 SDK x64 bin directory
$KitBinRoot = "C:\Program Files (x86)\Windows Kits\10\bin"
$SdkDir = Get-ChildItem -Path $KitBinRoot -Directory -ErrorAction Stop |
    Where-Object { Test-Path (Join-Path $_.FullName "x64\makeappx.exe") } |
    Sort-Object { [version]$_.Name } -Descending |
    Select-Object -First 1

if (-not $SdkDir) {
    throw "Windows SDK x64 tools (makeappx.exe, makepri.exe, signtool.exe) not found under '$KitBinRoot'."
}

$SdkX64 = Join-Path $SdkDir.FullName "x64"
$MakePri = Join-Path $SdkX64 "makepri.exe"
$MakeAppx = Join-Path $SdkX64 "makeappx.exe"
$SignTool = Join-Path $SdkX64 "signtool.exe"
Write-Host "Using Windows SDK $($SdkDir.Name) ($SdkX64)"

Push-Location $RepoRoot
try {
    $AssetsDir = Join-Path $ScriptDir "Assets"
    if (-not (Test-Path (Join-Path $AssetsDir "StoreLogo.png"))) {
        Write-Host "Generating MSIX visual assets..."
        python (Join-Path $ScriptDir "generate_assets.py")
        if ($LASTEXITCODE -ne 0) { throw "generate_assets.py failed." }
    }

    if (-not $SkipBuild) {
        $prevRustFlags = $env:RUSTFLAGS
        try {
            if (-not $DynamicCrt) {
                $env:RUSTFLAGS = "-C target-feature=+crt-static"
                Write-Host "Building release binaries with static CRT (+crt-static)..."
            } else {
                Write-Host "Building release binaries with dynamic CRT..."
            }
            $cargoArgs = @("build", "--release", "--locked")
            if (-not $IncludeDonateButton) {
                $cargoArgs += "--no-default-features"
            }
            & cargo @cargoArgs
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed." }
        } finally {
            $env:RUSTFLAGS = $prevRustFlags
        }
    }

    $DistDir = Join-Path $RepoRoot "dist"
    $StageDir = Join-Path $DistDir "msix_stage"
    $TmpDir = Join-Path $DistDir "msix_tmp"
    Remove-Item -Recurse -Force $StageDir, $TmpDir -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $StageDir, $TmpDir | Out-Null

    foreach ($bin in @("openrecover.exe", "openrecover-cli.exe")) {
        $src = Join-Path $RepoRoot "target\release\$bin"
        if (-not (Test-Path $src)) { throw "Missing release binary: $src" }
        Copy-Item $src $StageDir
    }
    Copy-Item (Join-Path $RepoRoot "LICENSE"), (Join-Path $RepoRoot "README.md"), (Join-Path $RepoRoot "PRIVACY.md") $StageDir
    Copy-Item -Recurse $AssetsDir (Join-Path $StageDir "Assets")

    $template = Get-Content (Join-Path $ScriptDir "AppxManifest.template.xml") -Raw -Encoding UTF8
    $esc = { param([string]$v) [System.Security.SecurityElement]::Escape($v) }
    $manifestXml = $template.
        Replace("__IDENTITY_NAME__", (& $esc $IdentityName)).
        Replace("__PUBLISHER__", (& $esc $Publisher)).
        Replace("__PUBLISHER_DISPLAY_NAME__", (& $esc $PublisherDisplayName)).
        Replace("__VERSION__", (& $esc $Version))

    $ManifestPath = Join-Path $StageDir "AppxManifest.xml"
    [System.IO.File]::WriteAllText($ManifestPath, $manifestXml, [System.Text.UTF8Encoding]::new($false))

    $PriConfig = Join-Path $TmpDir "priconfig.xml"
    & $MakePri createconfig /cf $PriConfig /dq en-US /pv 10.0.0 /o | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "makepri createconfig failed." }

    # Merge all resource scales/languages into a single resources.pri for single-MSIX Store submission
    [xml]$priXml = Get-Content $PriConfig -Encoding UTF8
    $pkgNode = $priXml.resources.SelectSingleNode("packaging")
    if ($pkgNode) { [void]$priXml.resources.RemoveChild($pkgNode) }
    $priXml.Save($PriConfig)

    & $MakePri new /pr $StageDir /cf $PriConfig /mn $ManifestPath /of (Join-Path $StageDir "resources.pri") /o
    if ($LASTEXITCODE -ne 0) { throw "makepri new failed." }

    $OutMsix = Join-Path $DistDir "OpenRecover-$Version-x64.msix"
    & $MakeAppx pack /d $StageDir /p $OutMsix /o
    if ($LASTEXITCODE -ne 0) { throw "makeappx pack failed." }

    if ($SignSelfSigned) {
        $cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
            Where-Object { $_.Subject -eq $Publisher } |
            Select-Object -First 1
        if (-not $cert) {
            Write-Host "Creating self-signed test certificate for $Publisher..."
            $cert = New-SelfSignedCertificate `
                -Type Custom `
                -Subject $Publisher `
                -KeyUsage DigitalSignature `
                -FriendlyName "OpenRecover MSIX Local Test" `
                -CertStoreLocation "Cert:\CurrentUser\My" `
                -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
        }
        $CerPath = Join-Path $DistDir "OpenRecover-DevTest.cer"
        Export-Certificate -Cert $cert -FilePath $CerPath -Force | Out-Null

        & $SignTool sign /fd SHA256 /sha1 $cert.Thumbprint $OutMsix
        if ($LASTEXITCODE -ne 0) { throw "signtool sign failed." }
        Write-Host "Signed test certificate exported to: $CerPath"
    } else {
        Write-Host "Left MSIX unsigned (ready for Microsoft Partner Center Store upload & Microsoft Store signing)."
    }

    Write-Host "MSIX ready: $OutMsix"
} finally {
    Pop-Location
}
