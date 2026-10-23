#Requires -Version 7.0
<#
.SYNOPSIS
    Builds the Windows release of ai-translator: a portable ZIP with the binary,
    the media engine DLL and the licences.

.DESCRIPTION
    Windows is built on Windows, with the MSVC toolchain: the engine is a native
    library in MSVC format and the portable package has to run on a machine
    without a compiler. Everything is staged under build\windows and the result
    lands in dist\ next to the other platforms.

    The voice-chat sink is part of every package: the feature is fixed here, not
    a switch, and the media engine DLL travels beside the executable.

.PARAMETER Version
    Release version; taken from Cargo.toml when omitted.

.PARAMETER SkipTests
    Skip `cargo test` before packaging. The default runs it, because a package
    built from code that does not pass its own tests is not a release.

.EXAMPLE
    pwsh -File packaging/build-windows.ps1
#>
[CmdletBinding()]
param(
    [string]$Version = '',
    [switch]$SkipTests,
    [switch]$NoMsi
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $Version) {
    $match = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"'
    if (-not $match) {
        throw 'cannot read the version from Cargo.toml'
    }
    $Version = $match.Matches[0].Groups[1].Value
}

Write-Host "== ai-translator $Version for Windows x86_64 =="

$target = 'x86_64-pc-windows-msvc'
$env:CARGO_TARGET_DIR = Join-Path $root 'build\windows\target'
# A package without the voice-chat sink is not shipped, so there is no switch.
$features = @('telegram-call')

$cargoCommon = @('--locked', '--target', $target)

if (-not $SkipTests) {
    Write-Host '-- tests'
    $testArgs = @('test', '--release') + $cargoCommon
    if ($features.Count -gt 0) {
        $testArgs += @('--features', ($features -join ','))
    }
    & cargo @testArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo test failed with exit code $LASTEXITCODE"
    }
}

$engine = & (Join-Path $PSScriptRoot 'fetch-ntgcalls.ps1')
$env:NTGCALLS_DYLIB = '1'
$env:NTGCALLS_LIB_DIR = $engine.LibDir

Write-Host '-- release build'
$buildArgs = @('build', '--release') + $cargoCommon
if ($features.Count -gt 0) {
    $buildArgs += @('--features', ($features -join ','))
}
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

$stage = Join-Path $root "build\windows\work\ai-translator-$Version"
if (Test-Path $stage) {
    Remove-Item -Recurse -Force $stage
}
New-Item -ItemType Directory -Force -Path $stage | Out-Null

$exe = Join-Path $env:CARGO_TARGET_DIR "$target\release\ai-translator.exe"
if (-not (Test-Path $exe)) {
    throw "the executable was not produced: $exe"
}
Copy-Item $exe (Join-Path $stage 'ai-translator.exe')

Copy-Item $engine.DllPath (Join-Path $stage 'ntgcalls.dll')

foreach ($file in @('LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE', 'README.md')) {
    $source = Join-Path $root $file
    if (Test-Path $source) {
        Copy-Item $source (Join-Path $stage $file)
    }
}
Copy-Item (Join-Path $PSScriptRoot 'INSTALL-windows.txt') (Join-Path $stage 'INSTALL-windows.txt')

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null

$zip = Join-Path $dist "ai-translator-$Version-x86_64-windows.zip"
if (Test-Path $zip) {
    Remove-Item -Force $zip
}
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal

$hash = (Get-FileHash -Path $zip -Algorithm SHA256).Hash.ToLowerInvariant()
$line = "$hash  $(Split-Path -Leaf $zip)"
Set-Content -Path "$zip.sha256" -Value $line -Encoding ascii

Write-Host "== ready: $zip"
Write-Host "   $line"

# The installer wraps the same staged files, so it is built from the directory
# that is already complete.
if (-not $NoMsi) {
    & (Join-Path $PSScriptRoot 'build-msi.ps1') -Stage $stage -Version $Version
    if ($LASTEXITCODE -ne 0) {
        throw "the installer was not built: exit code $LASTEXITCODE"
    }
}
