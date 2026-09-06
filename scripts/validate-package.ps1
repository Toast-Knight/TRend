param(
    [ValidateSet('windows', 'linux', 'android')]
    [string]$Platform = 'windows',

    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)

$ErrorActionPreference = 'Stop'
$package = if ($Platform -eq 'android') {
    Join-Path $RepositoryRoot 'dist/android/aarch64-linux-android'
} else {
    Join-Path $RepositoryRoot "dist/$Platform"
}
$expected = switch ($Platform) {
    'windows' { @(
        'bin/trend.dll', 'bin/trend-test-viewer.exe', 'bin/trend-python-tui-bridge.exe',
        'include/trend.h', 'lib/trend.dll.lib',
        'examples/terminal-overlay/play-top-right.bat',
        'examples/terminal-overlay/play-top-right.ps1', 'samples/ColorTest.png',
        'samples/ColorTest.gif', 'samples/ColorTest.mp4', 'docs/bridge-protocol.md',
        'README.md', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'manifest.sha256'
    ) }
    'linux' { @(
        'bin/trend-test-viewer', 'bin/trend-python-tui-bridge', 'include/trend.h',
        'lib/libtrend.so', 'samples/ColorTest.png', 'samples/ColorTest.gif',
        'docs/bridge-protocol.md', 'README.md', 'LICENSE', 'NOTICE',
        'THIRD_PARTY_NOTICES.md', 'manifest.sha256'
    ) }
    'android' { @(
        'bin/trend-test-viewer', 'include/trend.h', 'lib/libtrend.so', 'README.md',
        'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', 'manifest.sha256'
    ) }
}

$actual = Get-ChildItem -LiteralPath $package -Recurse -File | ForEach-Object {
    $_.FullName.Substring($package.Length + 1).Replace('\', '/')
}
$missing = $expected | Where-Object { $_ -notin $actual }
$unexpected = $actual | Where-Object { $_ -notin $expected }
if ($missing) { throw "Missing package files: $($missing -join ', ')" }
if ($unexpected) { throw "Unexpected package files: $($unexpected -join ', ')" }
if ($actual | Where-Object Extension -In '.pdb', '.exp') {
    throw 'Debug or linker intermediate files were included in the package.'
}

$manifestPath = Join-Path $package 'manifest.sha256'
$manifestEntries = Get-Content -LiteralPath $manifestPath
$manifestFiles = @($expected | Where-Object { $_ -ne 'manifest.sha256' } | Sort-Object)
if ($manifestEntries.Count -ne $manifestFiles.Count) {
    throw 'Package manifest entry count does not match the package layout.'
}
for ($index = 0; $index -lt $manifestFiles.Count; $index++) {
    $relativePath = $manifestFiles[$index]
    $hash = (Get-FileHash -LiteralPath (Join-Path $package $relativePath) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($manifestEntries[$index] -ne "$hash  $relativePath") {
        throw "Package manifest mismatch: $relativePath"
    }
}

if ($Platform -eq 'windows' -or ($Platform -eq 'linux' -and $IsLinux)) {
    $executableSuffix = if ($Platform -eq 'windows') { '.exe' } else { '' }
    Push-Location $package
    try {
        & "./bin/trend-test-viewer$executableSuffix" --version | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Packaged viewer failed.' }
        & "./bin/trend-python-tui-bridge$executableSuffix" --version | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Packaged bridge failed.' }
        & "./bin/trend-test-viewer$executableSuffix" --probe 20x10 2 './samples/ColorTest.png' | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Packaged PNG probe failed.' }
        if ($Platform -eq 'windows') {
            & './bin/trend-test-viewer.exe' --probe 20x10 2 './samples/ColorTest.mp4' | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Packaged MP4 probe failed.' }
        }
    } finally {
        Pop-Location
    }
}

Write-Host "Package validation passed: $package"