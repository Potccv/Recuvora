param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Web', 'Desktop', 'All')]
    [string]$Target,

    [Parameter(Mandatory = $true)]
    [string]$BuildDir,

    [string]$NodePath = 'node',
    [string]$CargoPath = 'cargo',
    [ValidateSet('Debug', 'Release')]
    [string]$Profile = 'Release'
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

function Get-ExternalDirectory([string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value)) { throw 'An external build directory is required.' }
    $resolved = [IO.Path]::GetFullPath($Value)
    $projectPrefix = $projectRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if ($resolved.Equals($projectRoot, [StringComparison]::OrdinalIgnoreCase) -or
        $resolved.StartsWith($projectPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "UI build output must be outside the source directory: $resolved"
    }

    $existingPath = $resolved
    while (-not (Test-Path -LiteralPath $existingPath)) {
        $existingPath = [IO.Path]::GetDirectoryName($existingPath)
        if ([string]::IsNullOrEmpty($existingPath)) { throw 'Build directory has no existing parent.' }
    }
    $ancestor = Get-Item -LiteralPath $existingPath
    while ($null -ne $ancestor) {
        if (($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Linked build directory is not supported: $($ancestor.FullName)"
        }
        $ancestor = $ancestor.Parent
    }
    New-Item -ItemType Directory -Force -Path $resolved | Out-Null
    return $resolved
}

function Invoke-Checked([scriptblock]$Command, [string]$Failure) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw $Failure }
}

$resolvedBuildDir = Get-ExternalDirectory $BuildDir
$uiSource = Join-Path $projectRoot 'src\interfaces\ui\public'
$javascriptFiles = @(
    Get-ChildItem -LiteralPath $uiSource -Recurse -File |
        Where-Object { $_.Extension -in @('.js', '.mjs') } |
        Sort-Object FullName
)

foreach ($file in $javascriptFiles) {
    Invoke-Checked { & $NodePath --check $file.FullName } "JavaScript syntax check failed: $($file.FullName)"
}
Invoke-Checked { & $NodePath --check (Join-Path $PSScriptRoot 'build-ui.mjs') } 'UI build script syntax check failed.'

$buildResultText = & $NodePath (Join-Path $PSScriptRoot 'build-ui.mjs') `
    --source $uiSource `
    --output-root $resolvedBuildDir
if ($LASTEXITCODE -ne 0) { throw 'Shared UI asset build failed.' }
$buildResult = $buildResultText | ConvertFrom-Json

$desktopBinary = $null
if ($Target -in @('Desktop', 'All')) {
    $savedTarget = $env:CARGO_TARGET_DIR
    try {
        $env:CARGO_TARGET_DIR = Get-ExternalDirectory (Join-Path $resolvedBuildDir 'cargo')
        $cargoArguments = @(
            'build', '--manifest-path', (Join-Path $projectRoot 'Cargo.toml'), '--locked', '--no-default-features',
            '--features', 'desktop-ui', '--bin', 'recuvora-desktop'
        )
        if ($Profile -eq 'Release') { $cargoArguments += '--release' }
        & $CargoPath @cargoArguments
        if ($LASTEXITCODE -ne 0) { throw 'Desktop UI build failed.' }
        $profileDirectory = if ($Profile -eq 'Release') { 'release' } else { 'debug' }
        $binaryName = if ($env:OS -eq 'Windows_NT') { 'recuvora-desktop.exe' } else { 'recuvora-desktop' }
        $desktopBinary = Join-Path (Join-Path $env:CARGO_TARGET_DIR $profileDirectory) $binaryName
        if (-not (Test-Path -LiteralPath $desktopBinary -PathType Leaf)) {
            throw "Desktop UI build did not produce the expected binary: $desktopBinary"
        }
        $desktopBinary = (Get-Item -LiteralPath $desktopBinary).FullName
    } finally {
        $env:CARGO_TARGET_DIR = $savedTarget
    }
}

if (Test-Path -LiteralPath (Join-Path $projectRoot 'gen')) {
    throw 'Tauri generated files must stay in the external Cargo output directory; source gen/ is present.'
}

[ordered]@{
    status = 'ok'
    target = $Target.ToLowerInvariant()
    build_id = $buildResult.build_id
    web_directory = if ($Target -in @('Web', 'All')) { $buildResult.output_dir } else { $null }
    desktop_binary = $desktopBinary
    asset_manifest = $buildResult.manifest
} | ConvertTo-Json -Compress
