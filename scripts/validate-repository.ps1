param(
    [switch]$StaticOnly,
    [switch]$IncludeLinux,
    [switch]$IncludeAndroid
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $repositoryRoot

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments
    )

    Write-Host "> $Command $($Arguments -join ' ')"
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $Command"
    }
}

try {
    $legacyPaths = @(
        'APIS.md',
        'ANDROID.md',
        'APPLICATION_INTEGRATION.md',
        'TREND_USAGE_GUIDE.md',
        'Test runs Envs',
        'build_all_targets.bat',
        'build_android.bat',
        'build_linux.bat',
        'build_windows.bat',
        'test_linux_wsl2.bat',
        'test_windows.bat',
        'visual_test_linux_wsl2.bat',
        'visual_test_windows.bat'
    )
    $remainingLegacyPaths = @($legacyPaths | Where-Object { Test-Path -LiteralPath $_ })
    if ($remainingLegacyPaths.Count -ne 0) {
        throw "Legacy repository paths remain: $($remainingLegacyPaths -join ', ')"
    }

    $documentation = Get-ChildItem -Recurse -File -Filter '*.md' |
        Where-Object { $_.FullName -notmatch '[\\/](target|dist|Builds Organized|android-ndk-r27d)[\\/]' }
    $brokenLinks = @()
    foreach ($file in $documentation) {
        $text = Get-Content -Raw -LiteralPath $file.FullName
        foreach ($match in [regex]::Matches($text, '\[[^\]]+\]\((?!https?://|mailto:|#)([^)#]+)(?:#[^)]+)?\)')) {
            $relativeTarget = [uri]::UnescapeDataString($match.Groups[1].Value)
            $target = Join-Path $file.DirectoryName $relativeTarget
            if (-not (Test-Path -LiteralPath $target)) {
                $brokenLinks += "$($file.FullName.Substring($repositoryRoot.Length + 1)) -> $relativeTarget"
            }
        }
    }
    if ($brokenLinks.Count -ne 0) {
        throw "Broken Markdown links:`n$($brokenLinks -join "`n")"
    }

    Write-Host 'Repository structure and Markdown links passed.'
    if ($StaticOnly) {
        exit 0
    }

    Invoke-Checked cargo check --workspace --all-targets --all-features
    Invoke-Checked cmd.exe /d /c scripts\test-windows.bat
    Invoke-Checked python -m unittest examples\python-tui\test_trend_tui.py examples\python-video-player\test_player.py

    Write-Host '> first-render smoke test'
    & cargo run --quiet -p trend-first-render -- tests/fixtures/media/ColorTest.png | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'First-render smoke test failed.' }

    Write-Host '> terminal overlay smoke test'
    & powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File examples\terminal-overlay\play-top-right.ps1 -Media tests\fixtures\media\ColorTest.gif -Width 15 -Height 15 -RunMilliseconds 200 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Terminal overlay smoke test failed.' }

    Invoke-Checked cmd.exe /d /c scripts\build-windows.bat

    if ($IncludeLinux) {
        Invoke-Checked cmd.exe /d /c scripts\build-linux-wsl2.bat
        Invoke-Checked cmd.exe /d /c scripts\test-linux-wsl2.bat
    }
    if ($IncludeAndroid) {
        Invoke-Checked cmd.exe /d /c scripts\build-android.bat
    }

    Write-Host 'Repository validation passed.'
}
finally {
    Pop-Location
}