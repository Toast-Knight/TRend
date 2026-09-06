param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)

$ErrorActionPreference = 'Stop'
$destination = Join-Path $RepositoryRoot 'dist\windows'
$files = [ordered]@{
    'target\release\trend.dll' = 'bin\trend.dll'
    'target\release\trend-test-viewer.exe' = 'bin\trend-test-viewer.exe'
    'target\release\trend-python-tui-bridge.exe' = 'bin\trend-python-tui-bridge.exe'
    'include\trend.h' = 'include\trend.h'
    'target\release\trend.dll.lib' = 'lib\trend.dll.lib'
    'examples\terminal-overlay\play-top-right.bat' = 'examples\terminal-overlay\play-top-right.bat'
    'examples\terminal-overlay\play-top-right.ps1' = 'examples\terminal-overlay\play-top-right.ps1'
    'tests\fixtures\media\ColorTest.png' = 'samples\ColorTest.png'
    'tests\fixtures\media\ColorTest.gif' = 'samples\ColorTest.gif'
    'tests\fixtures\media\ColorTest.mp4' = 'samples\ColorTest.mp4'
    'docs\bridge-protocol.md' = 'docs\bridge-protocol.md'
    'packaging\windows\README.md' = 'README.md'
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

& (Join-Path $PSScriptRoot 'validate-package.ps1') -Platform windows -RepositoryRoot $RepositoryRoot
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "Windows package: $destination"