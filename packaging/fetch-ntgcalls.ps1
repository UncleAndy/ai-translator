#Requires -Version 7.0
<#
.SYNOPSIS
    Fetches the native media engine of the Telegram voice-chat sink for Windows.

.DESCRIPTION
    Downloads the prebuilt ntgcalls release, checks its SHA256 against the value
    pinned below and unpacks it. The version is fixed on purpose: the checksum
    belongs to one archive, and an unknown version cannot be verified, so it is
    refused instead of being downloaded blindly.

    The engine for Windows ships as a DLL with an import library, so the release
    beside it is a small executable plus `ntgcalls.dll`, the same way the macOS
    build carries its dylib.

    Returns an object with `LibDir` (the directory holding the import library)
    and `DllPath` (the DLL that has to sit next to the executable).

.PARAMETER Version
    Engine version; only the pinned one is accepted.

.PARAMETER OutDir
    Where to unpack. Defaults to build\ntgcalls\<target>.

.EXAMPLE
    $engine = ./packaging/fetch-ntgcalls.ps1
    $engine.LibDir
#>
[CmdletBinding()]
param(
    [string]$Version = '3.0.4',
    [string]$OutDir = ''
)

$ErrorActionPreference = 'Stop'

# Pinned assets: a checksum is only meaningful for the exact archive it was
# taken from, so a new engine version needs a new line here.
$Pinned = @{
    '3.0.4' = @{
        Asset  = 'ntgcalls.windows-x86_64-shared_libs.zip'
        Sha256 = 'fe5bbc2600119633ab8459f8f328444f8f3e19e3cab431808293a42cdc5a23eb'
        LibSub = 'lib/Release'
    }
}

if (-not $Pinned.ContainsKey($Version)) {
    [Console]::Error.WriteLine("engine version $Version has no pinned checksum; add it to fetch-ntgcalls.ps1")
    exit 3
}

$pin = $Pinned[$Version]
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) {
    $OutDir = Join-Path $root "build\ntgcalls\windows-x86_64-$Version"
}

$libDir = Join-Path $OutDir $pin.LibSub
$dllPath = Join-Path $OutDir (Join-Path $pin.LibSub 'ntgcalls.dll')

if (-not (Test-Path $dllPath)) {
    $url = "https://github.com/pytgcalls/ntgcalls/releases/download/v$Version/$($pin.Asset)"
    $archive = Join-Path $OutDir $pin.Asset

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

    Write-Host "-- downloading $($pin.Asset)"
    Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing

    $actual = (Get-FileHash -Path $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $pin.Sha256) {
        Remove-Item -Force $archive
        [Console]::Error.WriteLine("checksum mismatch for $($pin.Asset): expected $($pin.Sha256), got $actual")
        exit 4
    }

    Expand-Archive -Path $archive -DestinationPath $OutDir -Force
    Remove-Item -Force $archive
}

if (-not (Test-Path $dllPath)) {
    [Console]::Error.WriteLine("the archive did not contain $($pin.LibSub)/ntgcalls.dll")
    exit 5
}

Write-Host "-- engine ready: $dllPath"
[pscustomobject]@{
    LibDir  = (Resolve-Path $libDir).Path
    DllPath = (Resolve-Path $dllPath).Path
    Version = $Version
}
