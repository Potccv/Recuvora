param(
    [Parameter(Mandatory = $true)][string]$BuildDir,
    [Parameter(Mandatory = $true)][string]$TempRoot,
    [string]$CargoPath = 'cargo'
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

function Get-ExternalDirectory([string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value)) { throw 'An external directory is required.' }
    $resolved = [IO.Path]::GetFullPath($Value)
    $projectPrefix = $projectRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if ($resolved.Equals($projectRoot, [StringComparison]::OrdinalIgnoreCase) -or
        $resolved.StartsWith($projectPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Build and test data must be outside the source directory: $resolved"
    }
    # Check existing ancestors before creating anything through a link.
    $existingPath = $resolved
    while (-not (Test-Path -LiteralPath $existingPath)) {
        $existingPath = [IO.Path]::GetDirectoryName($existingPath)
        if ([string]::IsNullOrEmpty($existingPath)) { throw 'No existing parent directory.' }
    }
    $ancestor = Get-Item -LiteralPath $existingPath
    while ($null -ne $ancestor) {
        if (($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Linked build or test directory is not supported: $($ancestor.FullName)"
        }
        $ancestor = $ancestor.Parent
    }
    New-Item -ItemType Directory -Force -Path $resolved | Out-Null
    return $resolved
}

$savedTarget = $env:CARGO_TARGET_DIR
$savedTemp = $env:RECUVORA_TEST_TEMP
$savedThreads = $env:RUST_TEST_THREADS
try {
    $env:CARGO_TARGET_DIR = Get-ExternalDirectory $BuildDir
    $env:RECUVORA_TEST_TEMP = Get-ExternalDirectory $TempRoot
    $env:RUST_TEST_THREADS = '4'
    Push-Location $projectRoot
    try {
        & $CargoPath fmt --all -- --check
        if ($LASTEXITCODE -ne 0) { throw 'Rust formatting check failed.' }
        & $CargoPath clippy --all-targets --features server,web-ui --locked -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
        & $CargoPath test --all-targets --features server,web-ui --locked
        if ($LASTEXITCODE -ne 0) { throw 'Default package tests failed.' }
    } finally {
        Pop-Location
    }
} finally {
    $env:CARGO_TARGET_DIR = $savedTarget
    $env:RECUVORA_TEST_TEMP = $savedTemp
    $env:RUST_TEST_THREADS = $savedThreads
}
