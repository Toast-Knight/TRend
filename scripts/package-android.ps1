param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)

$ErrorActionPreference = 'Stop'
$destination = Join-Path $RepositoryRoot 'dist/android/aarch64-linux-android'
$files = [ordered]@{
    'target/aarch64-linux-android/release/libtrend.so' = 'lib/libtrend.so'
    'target/aarch64-linux-android/release/trend-test-viewer' = 'bin/trend-test-viewer'
    'include/trend.h' = 'include/trend.h'
    'docs/platforms/android.md' = 'README.md'
    'LICENSE' = 'LICENSE'
    'NOTICE' = 'NOTICE'
    'THIRD_PARTY_NOTICES.md' = 'THIRD_PARTY_NOTICES.md'
}

if (Test-Path -LiteralPath $destination) {
    Remove-Item -LiteralPath $destination -Recurse -Force
}

foreach ($entry in $files.GetEnumerator()) {
    $source = Join-Path $RepositoryRoot $entry.Key
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Required package input is missing: $($entry.Key)"
    }
    $target = Join-Path $destination $entry.Value
    $null = New-Item -ItemType Directory -Force -Path (Split-Path $target)
    Copy-Item -LiteralPath $source -Destination $target
}

$manifest = foreach ($relativePath in ($files.Values | Sort-Object)) {
    $path = Join-Path $destination $relativePath
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($relativePath.Replace('\', '/'))"
}
$manifest | Set-Content -LiteralPath (Join-Path $destination 'manifest.sha256') -Encoding ASCII

& (Join-Path $PSScriptRoot 'validate-package.ps1') -Platform android -RepositoryRoot $RepositoryRoot
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "Android package: $destination"