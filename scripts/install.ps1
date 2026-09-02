# Install io-workbench from a GitHub Release into the current user's profile.
#
# The archive is verified against the SHA256SUMS asset before its binaries are
# copied. Starting a server remains an explicit choice after installation.

[CmdletBinding()]
param(
    [string]$Version = $env:IO_WORKBENCH_VERSION,
    [string]$Repository = $env:IO_WORKBENCH_REPOSITORY,
    [string]$InstallDir = $env:IO_WORKBENCH_INSTALL_DIR,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$RemainingArguments
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Note {
    param([string]$Message)
    Write-Host "io-workbench installer: $Message"
}

function Show-Usage {
    @'
Usage: install.ps1 [-Version <tag>]

Installs the matching io-workbench GitHub Release for this Windows computer.

Options:
  -Version <tag>    Install a release such as v0.1.0 (or 0.1.0).
  --version <tag>   POSIX-style spelling of -Version.
  -Help             Show this help.

Environment overrides:
  IO_WORKBENCH_VERSION       Same as -Version.
  IO_WORKBENCH_INSTALL_DIR   Destination for io-workbench.exe and iowb.exe.
  IO_WORKBENCH_REPOSITORY    GitHub owner/repository (advanced use).
'@ | Write-Host
}

function Get-RemainingOptionValue {
    param(
        [string[]]$Arguments,
        [ref]$Index,
        [string]$OptionName
    )

    if (($Index.Value + 1) -ge $Arguments.Count) {
        throw "$OptionName needs a release tag."
    }

    $Index.Value++
    return $Arguments[$Index.Value]
}

if ($RemainingArguments) {
    for ($argumentIndex = 0; $argumentIndex -lt $RemainingArguments.Count; $argumentIndex++) {
        $argument = $RemainingArguments[$argumentIndex]
        switch -Regex ($argument) {
            '^(--version|--Version)$' {
                $Version = Get-RemainingOptionValue -Arguments $RemainingArguments -Index ([ref]$argumentIndex) -OptionName '--version'
                continue
            }
            '^--version=(.+)$' {
                $Version = $Matches[1]
                continue
            }
            '^(--help|--Help|-h)$' {
                Show-Usage
                exit 0
            }
            default {
                throw "Unknown option: $argument. Run with -Help for usage."
            }
        }
    }
}

if ([Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw 'This installer is for Windows. Use install.sh on Linux or macOS.'
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = 'latest'
}
if ([string]::IsNullOrWhiteSpace($Repository)) {
    $Repository = 'giofahreza/io-workbench'
}
if ($Repository -notmatch '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$') {
    throw 'Repository must be in owner/repository form.'
}

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\io-workbench'
}

$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
switch ($architecture) {
    'X64' { $target = 'windows-x86_64' }
    'Arm64' { $target = 'windows-aarch64' }
    default { throw "Unsupported Windows CPU architecture: $architecture." }
}

if ($Version -eq 'latest') {
    Write-Note "Resolving the latest release from $Repository."
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/releases/latest" -Headers @{ 'User-Agent' = 'io-workbench-installer' }
    $tag = [string]$release.tag_name
    if ([string]::IsNullOrWhiteSpace($tag)) {
        throw 'Could not read tag_name from the GitHub release response.'
    }
} elseif ($Version -match '^v[0-9][A-Za-z0-9._-]*$') {
    $tag = $Version
} elseif ($Version -match '^[0-9][A-Za-z0-9._-]*$') {
    $tag = "v$Version"
} else {
    throw "Invalid release version: $Version"
}

if ($tag -notmatch '^v[0-9][A-Za-z0-9._-]*$') {
    throw "Invalid GitHub release tag: $tag"
}

$assetName = "io-workbench-$tag-$target.zip"
$releaseBase = "https://github.com/$Repository/releases/download/$tag"
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ("io-workbench-install-" + [Guid]::NewGuid().ToString('N'))
$archive = Join-Path $tempDir $assetName
$sumsFile = Join-Path $tempDir 'SHA256SUMS'
$extractDir = Join-Path $tempDir 'package'

try {
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
    Write-Note "Downloading $assetName."
    Invoke-WebRequest -Uri "$releaseBase/$assetName" -OutFile $archive -UseBasicParsing
    Invoke-WebRequest -Uri "$releaseBase/SHA256SUMS" -OutFile $sumsFile -UseBasicParsing

    $sumLine = Get-Content -LiteralPath $sumsFile | Where-Object {
        $_ -match ('^([0-9A-Fa-f]{64})\s+\*?' + [regex]::Escape($assetName) + '$')
    } | Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($sumLine)) {
        throw "SHA256SUMS does not contain $assetName."
    }

    $expectedSha256 = (($sumLine -split '\s+')[0]).ToLowerInvariant()
    $actualSha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $expectedSha256) {
        throw "Checksum verification failed for $assetName."
    }
    Write-Note 'Release checksum verified.'

    Expand-Archive -LiteralPath $archive -DestinationPath $extractDir -Force
    foreach ($requiredFile in @('io-workbench.exe', 'iowb.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $extractDir $requiredFile) -PathType Leaf)) {
            throw "Release archive is missing required file: $requiredFile"
        }
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $extractDir 'io-workbench.exe') -Destination (Join-Path $InstallDir 'io-workbench.exe') -Force
    Copy-Item -LiteralPath (Join-Path $extractDir 'iowb.exe') -Destination (Join-Path $InstallDir 'iowb.exe') -Force
} finally {
    if (Test-Path -LiteralPath $tempDir) {
        Remove-Item -LiteralPath $tempDir -Recurse -Force
    }
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathItems = @()
if (-not [string]::IsNullOrWhiteSpace($userPath)) {
    $pathItems = $userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
}
$hasInstallDir = $pathItems | Where-Object { $_.TrimEnd('\') -ieq $InstallDir.TrimEnd('\') } | Select-Object -First 1
if (-not $hasInstallDir) {
    $newUserPath = (($pathItems + $InstallDir) -join ';')
    [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
    $env:Path = "$InstallDir;$env:Path"
    Write-Note "Added $InstallDir to your user PATH. Open a new terminal after this one."
}

Write-Note "Installed io-workbench.exe and iowb.exe to $InstallDir."
Write-Host ''
Write-Host 'Start a local, authenticated workbench when you are ready:'
Write-Host '  io-workbench start'
Write-Host ''
Write-Host 'Then open http://127.0.0.1:8787 and complete first-user setup.'
Write-Host 'This installer does not start a background service or expose the host to the network.'
