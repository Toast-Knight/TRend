param(
    [string]$Tag = $env:GITHUB_REF_NAME
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if ($Tag -notmatch '^v(?<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$') {
    throw "Release tag must use the form vMAJOR.MINOR.PATCH: $Tag"
}

$releaseVersion = $Matches.version
$metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    throw 'Unable to read Cargo workspace metadata.'
}

$versionMismatches = @($metadata.packages | Where-Object { $_.version -ne $releaseVersion })
if ($versionMismatches.Count -ne 0) {
    $details = $versionMismatches | ForEach-Object { "$($_.name)=$($_.version)" }
    throw "Workspace package versions do not match ${Tag}: $($details -join ', ')"
}

$changelog = Get-Content -Raw -LiteralPath (Join-Path $repositoryRoot 'CHANGELOG.md')
if ($changelog -notmatch "(?m)^## \[?$([regex]::Escape($releaseVersion))\]?(?:\s|$)") {
    throw "CHANGELOG.md has no section for $releaseVersion."
}

Write-Host "Release metadata passed: $Tag"