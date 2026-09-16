param(
    [Parameter(Mandatory = $true)][string]$BuildDir,
    [string]$NodePath = 'node',
    [string]$CargoPath = 'cargo'
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

& (Join-Path $PSScriptRoot 'build-ui.ps1') `
    -Target All `
    -BuildDir $BuildDir `
    -NodePath $NodePath `
    -CargoPath $CargoPath `
    -Profile Debug
if ($LASTEXITCODE -ne 0) { throw 'UI build checks failed.' }

$savedTarget = $env:CARGO_TARGET_DIR
$savedNode = $env:RECUVORA_NODE_PATH
try {
    $env:CARGO_TARGET_DIR = [IO.Path]::GetFullPath((Join-Path $BuildDir 'cargo'))
    $env:RECUVORA_NODE_PATH = $NodePath
    & $CargoPath test --manifest-path (Join-Path $projectRoot 'Cargo.toml') --locked --no-default-features --features web-ui --lib 'interfaces::ui::tests'
    if ($LASTEXITCODE -ne 0) { throw 'Shared UI invariant tests failed.' }
    & $CargoPath clippy --manifest-path (Join-Path $projectRoot 'Cargo.toml') --locked --no-default-features --features desktop-ui --bin recuvora-desktop -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Desktop UI Clippy check failed.' }
} finally {
    $env:CARGO_TARGET_DIR = $savedTarget
    $env:RECUVORA_NODE_PATH = $savedNode
}
