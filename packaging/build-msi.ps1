#Requires -Version 7.0
<#
.SYNOPSIS
    Builds the MSI installer of ai-translator from a staged directory.

.DESCRIPTION
    WiX is a Windows-only toolchain — its own CLI says so — so this script runs
    on Windows: locally, or on the windows-latest runner of CI.

    Version 5 is pinned on purpose: WiX 6 and later ask for the Open Source
    Maintenance Fee, while 5 needs nothing beyond the tool itself.

    The staged directory is the one `build-windows.ps1` assembles: the
    executable, the engine DLL, the licences and INSTALL-windows.txt.

.PARAMETER Stage
    Directory holding the files to install.

.PARAMETER Version
    Product version written into the MSI; taken from Cargo.toml when omitted.

.PARAMETER Out
    Where to put the MSI; defaults to dist\ai-translator-<version>-x86_64-windows.msi.

.EXAMPLE
    pwsh -File packaging/build-msi.ps1 -Stage build/windows/work/ai-translator-0.2.4
#>
[CmdletBinding()]
param(
    [string]$Stage = '',
    [string]$Version = '',
    [string]$Out = ''
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

if (-not $Stage) {
    $Stage = Join-Path $root "build\windows\work\ai-translator-$Version"
}
if (-not (Test-Path (Join-Path $Stage 'ai-translator.exe'))) {
    throw "no ai-translator.exe in $Stage; build the release first"
}
if (-not (Test-Path (Join-Path $Stage 'ntgcalls.dll'))) {
    Write-Warning "no ntgcalls.dll in ${Stage}: the installer would ship a program that cannot start"
}

if (-not $Out) {
    $Out = Join-Path $root "dist\ai-translator-$Version-x86_64-windows.msi"
}

if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    throw 'WiX is a .NET tool: install the .NET SDK, or build the archive only with -NoMsi'
}

$tools = Join-Path $env:USERPROFILE '.dotnet\tools'
$wix = Join-Path $tools 'wix.exe'
if (-not (Test-Path $wix)) {
    Write-Host '-- installing WiX 5'
    & dotnet tool install --global wix --version '5.*'
    if ($LASTEXITCODE -ne 0) {
        throw "dotnet tool install failed with exit code $LASTEXITCODE"
    }
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null

Write-Host "-- building the installer"
& $wix build -arch x64 -d "Version=$Version" -d "Stage=$Stage" -o $Out `
    (Join-Path $root 'packaging\ai-translator.wxs')
if ($LASTEXITCODE -ne 0) {
    throw "wix build failed with exit code $LASTEXITCODE"
}

$hash = (Get-FileHash -Path $Out -Algorithm SHA256).Hash.ToLowerInvariant()
$line = "$hash  $(Split-Path -Leaf $Out)"
Set-Content -Path "$Out.sha256" -Value $line -Encoding ascii

Write-Host "== installer: $Out"
Write-Host "   $line"
